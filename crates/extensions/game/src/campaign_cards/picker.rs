//! One picture per campaign in the game's campaign picker.
use super::{Card, cover, has_frame, tag_name};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use tags::rgba_bitmap::Template;
use vfs::Backend;

pub const CE: &str = "@picker/halo1";
pub const TWO: &str = "@picker/halo2";
pub const SIZE: [u16; 2] = [220, 207];
const CAIRO: &str = r"ui\code_global_bitmaps\campaign2";

fn tile(key: &str, map: &str, (sw, sh, pixels): (u32, u32, &[u8])) -> Card {
    let [width, height] = SIZE.map(u32::from);
    Card {
        map: map.into(),
        name: tag_name("game_picker", key),
        width,
        height,
        rgba: cover(sw, sh, pixels, width, height),
    }
}

fn halo1_tile(cards: &[Card]) -> Option<Card> {
    h1_contract::campaign::CAMPAIGN_ORDER
        .iter()
        .find_map(|level| {
            let name = tag_name("halo1", level);
            cards.iter().find(|c| c.name == name)
        })
        .map(|c| tile("halo1", CE, (c.width, c.height, &c.rgba)))
}

fn halo2_tile(disc: &Path) -> Result<Card> {
    let menu = vfs::open_disc(disc)?
        .host_file(super::MAINMENU)
        .context("Halo 2 disc has no maps/mainmenu.map")?;
    let mut map = map::Map::from_region(
        BufReader::new(File::open(menu.path)?),
        menu.offset,
        menu.len,
    )?;
    let id = map
        .tag(map::TagQuery::NameInGroup(CAIRO, map::FourCC(*b"bitm")))?
        .id;
    let bitmap = Template::read(&mut map, id)?;
    let (w, h) = bitmap.size();
    let pixels = bitmap.rgba(&mut map)?;
    anyhow::ensure!(w != 0 && h != 0, "{CAIRO} is empty");
    Ok(cairo_tile(w, h, &pixels))
}

fn cairo_tile(w: u16, h: u16, pixels: &[u8]) -> Card {
    // Strip the baked rim and cut corner before the menu rewrite
    // applies its own frame. Plain pictures already fill their bounds.
    const INSET: u16 = 16;
    if w > 2 * INSET && h > 2 * INSET && has_frame(pixels) {
        let x = usize::from(INSET) * 4;
        let interior: Vec<u8> = pixels
            .chunks_exact(usize::from(w) * 4)
            .skip(usize::from(INSET))
            .take(usize::from(h - 2 * INSET))
            .flat_map(|row| row[x..row.len() - x].iter().copied())
            .collect();
        tile(
            "halo2",
            TWO,
            (
                u32::from(w - 2 * INSET),
                u32::from(h - 2 * INSET),
                &interior,
            ),
        )
    } else {
        tile("halo2", TWO, (u32::from(w), u32::from(h), pixels))
    }
}

fn save_png(data: &Path, key: &str, art: &Card) -> Result<()> {
    let path = data
        .join("campaigns/game-picker")
        .join(format!("{key}.png"));
    let mut png = Vec::new();
    assets::png::write_rgba(&mut png, art.width, art.height, &art.rgba)?;
    if std::fs::read(&path).ok().as_ref() != Some(&png) {
        content::store::write_atomic(&path, &png)?;
    }
    Ok(())
}

/// Add each available game's tile independently, from the Halo 1 import's
/// first readable level card and Cairo Station on the Halo 2 disc.
pub fn prepare(data: &Path, disc: &Path, cards: &mut Vec<Card>) {
    let halo1 = halo1_tile(cards);
    if halo1.is_none() {
        eprintln!("campaigns: no Halo 1 level pictures (run `h2 import halo1` again)");
    }
    let halo2 = match halo2_tile(disc) {
        Ok(art) => Some(art),
        Err(e) => {
            eprintln!("campaigns: game picker Halo 2 artwork unavailable: {e:#}");
            None
        }
    };
    for (key, art) in [("halo1", halo1), ("halo2", halo2)] {
        if let Some(art) = art {
            if let Err(e) = save_png(data, key, &art) {
                eprintln!("campaigns: could not save {key} picker picture: {e:#}");
            }
            cards.push(art);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_card(level: &str, width: u32, height: u32, rgba: Vec<u8>) -> Card {
        Card {
            map: level.into(),
            name: tag_name("halo1", level),
            width,
            height,
            rgba,
        }
    }

    fn pixel(card: &Card, x: u32, y: u32) -> &[u8] {
        let at = ((y * card.width + x) * 4) as usize;
        &card.rgba[at..at + 4]
    }

    #[test]
    fn halo1_uses_the_first_available_mission_and_crops_its_image_once() {
        let w = 440;
        let h = 207;
        let wide = (0..w * h)
            .flat_map(|i| {
                let x = i % w;
                let y = i / w;
                [(x / 2) as u8, y as u8, (x % 2) as u8, 255]
            })
            .collect();
        let cards = [
            level_card("a50", 1, 1, vec![255; 4]),
            level_card("a30", w, h, wide),
        ];
        let art = halo1_tile(&cards).unwrap();
        assert_eq!(art.map, CE);
        assert_eq!([art.width, art.height], SIZE.map(u32::from));
        for y in 0..h {
            for x in 0..art.width {
                assert_eq!(pixel(&art, x, y), pixel(&cards[1], x + 110, y));
            }
        }
    }

    #[test]
    fn halo2_crops_out_the_baked_frame_but_keeps_plain_art() {
        let [w, h] = SIZE;
        let framed: Vec<u8> = (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    if x < 16 || x >= w - 16 || y < 16 || y >= h - 16 {
                        [255, 0, 0, 255]
                    } else {
                        [0, 80, 160, 191]
                    }
                })
            })
            .collect();
        let art = cairo_tile(w, h, &framed);
        assert_eq!(art.map, TWO);
        assert!(art.rgba.chunks_exact(4).all(|p| p == [0, 80, 160, 191]));

        let plain = [40, 80, 160, 255].repeat(usize::from(w) * usize::from(h));
        assert_eq!(cairo_tile(w, h, &plain).rgba, plain);
    }

    #[test]
    fn missing_halo2_art_and_an_unwritable_preview_do_not_drop_halo1_art() {
        // A missing disc and a file in place of the data directory.
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mut cards = vec![level_card("a30", 1, 1, vec![0, 80, 160, 255])];
        prepare(&data, &data.join("no-disc"), &mut cards);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[1].map, CE);
        assert_eq!(pixel(&cards[1], 0, 0), [0, 80, 160, 255]);
    }

    #[test]
    #[ignore = "reads the owner's Halo 2 disc"]
    fn picker_tiles_match_the_pngs_and_halo2_works_without_halo1_art() {
        let disc = std::env::var_os("H2_DISC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
            });
        let serial = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data = std::env::temp_dir().join(format!("h2-picker-{}-{serial}", std::process::id()));

        let mut cards = Vec::new();
        prepare(&data, &disc, &mut cards);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].map, TWO);

        let mut cards = vec![level_card(
            "a30",
            139,
            113,
            [80, 160, 0, 255].repeat(139 * 113),
        )];
        prepare(&data, &disc, &mut cards);
        assert_eq!(cards.len(), 3);
        let pair = &cards[1..];
        for (key, art) in ["halo1", "halo2"].into_iter().zip(pair) {
            let (w, h, rgba) =
                super::super::read_png(&data.join(format!("campaigns/game-picker/{key}.png")))
                    .unwrap();
            assert_eq!([w, h], SIZE.map(u32::from));
            assert_eq!(rgba, art.rgba);
        }
        assert_eq!((pair[0].map.as_str(), pair[1].map.as_str()), (CE, TWO));
        assert_ne!(pair[0].rgba, pair[1].rgba);
        std::fs::remove_dir_all(data).unwrap();
    }
}
