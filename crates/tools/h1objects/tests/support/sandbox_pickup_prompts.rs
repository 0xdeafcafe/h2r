use std::io::Cursor;

use h1convert::convert::halo1::{prepare_halo1, prepare_halo2};
use h1convert::convert::{Sandbox, structure_mopp};
use h1map::{Group, TagRef};
use h1objects::{base::Base, sandbox::Objects};
use map::{FourCC, Map, TagId, TagQuery};
use tags::{Language, Localization, StringId, Unic};

use super::{bloodgulch, h2_map};

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn block(map: &mut Map<Cursor<Vec<u8>>>, bytes: &[u8], offset: usize, stride: u32) -> Vec<u8> {
    let size = word(bytes, offset) * stride;
    if size == 0 {
        return Vec::new();
    }
    let file = map.file_offset(word(bytes, offset + 4), size).unwrap();
    map.read_file_range(file, u64::from(size)).unwrap()
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_pickup_text_survives_both_sandboxes() {
    let h1 = bloodgulch();
    let globals = h1.find(Group::new(b"hudg"), r"ui\hud\default").unwrap();
    let globals = h1.tag_data(globals).unwrap();
    let items = h1.tag(TagRef::read(globals, 0x94).unwrap().id).unwrap();
    let items = h1map::Block::read(h1.tag_data(items).unwrap(), 0).unwrap();
    let items = h1.block(items, 0x14).unwrap();
    let source_text = |row: usize| {
        let item = &items[row * 0x14..][..0x14];
        let text = h1.read(word(item, 0xc), word(item, 0) as usize).unwrap();
        let text: Vec<_> = text
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect();
        String::from_utf16(&text[..text.len() - 1]).unwrap()
    };

    for sandbox in [Sandbox::Halo1, Sandbox::Halo2] {
        let mut base = h2_map("coagulation");
        let templates = Base::read(&mut base).unwrap();
        let mut objects = Objects::new(&h1, &templates);
        let sha = content::Sha256::of(b"bloodgulch");
        let p = match sandbox {
            Sandbox::Halo1 => prepare_halo1(&h1, sha, &mut base, &mut objects),
            Sandbox::Halo2 => prepare_halo2(&h1, sha, &mut base, &mut objects),
        }
        .unwrap();
        let mopp = structure_mopp(&p.collision).unwrap();
        let fresh = p.build_fresh(&mut base, &mopp).unwrap();
        assert!(
            fresh.report.scratch.is_some(),
            "{:?}",
            fresh.report.scratch_failed
        );
        let mut out = Map::from_reader(Cursor::new(fresh.bytes)).unwrap();
        let scenario = out.scenario().unwrap().id;
        let scenario = out.read_tag_bytes(scenario).unwrap();
        let hud_id = TagId(word(&scenario, 0x20c));
        assert_eq!(word(&scenario, 0x208), FourCC(*b"hmt ").as_u32());
        assert_eq!(out.tag(hud_id).unwrap().group, FourCC(*b"hmt "));
        let hud = out.read_tag_bytes(hud_id).unwrap();
        let text = block(&mut out, &hud, 0, 1);
        let parts = block(&mut out, &hud, 8, 2);
        let rows = block(&mut out, &hud, 0x10, 0x40);
        let routes = block(&mut out, &hud, 0x9c, 0xc);
        let sprites = block(&mut out, &hud, 0xb0, 0x18);
        let subjects = block(&mut out, &hud, 0xb8, 8);
        let row = |index: u16| {
            &rows[(usize::from(half(&hud, 0x98)) + usize::from(index)) * 0x40..][..0x40]
        };
        let row_text = |index| {
            let units: Vec<_> = text[usize::from(half(row(index), 0x20)) * 2..]
                .as_chunks::<2>()
                .0
                .iter()
                .copied()
                .map(u16::from_le_bytes)
                .take_while(|&unit| unit != 0)
                .collect();
            String::from_utf16(&units).unwrap()
        };
        let route = |key| {
            routes
                .chunks_exact(0xc)
                .find(|r| word(r, 0) == key)
                .unwrap_or_else(|| panic!("{sandbox:?}: missing HUD route for {key:#x}"))
        };
        let mut checked = (0, 0);
        for tag in h1.tags() {
            let group = FourCC(tag.group.0.to_be_bytes());
            let is_weapon = group == FourCC(*b"weap");
            if !is_weapon && group != FourCC(*b"eqip") {
                continue;
            }
            let src = h1.tag_data(tag).unwrap();
            let index = half(src, 0x180) as i16;
            if index <= 0 || (!is_weapon && !matches!(half(src, 0x308), 2 | 3 | 5)) {
                continue;
            }
            let id = match out.tag(TagQuery::NameInGroup(&tag.name, group)) {
                Ok(tag) => tag.id,
                Err(map::Error::TagNotFound) if is_weapon => continue,
                Err(e) => panic!("{sandbox:?} {}: {e}", tag.name),
            };
            let built = out.read_tag_bytes(id).unwrap();
            let notice = source_text(index as usize);
            assert_eq!(
                row_text(half(route(word(&built, 0xe0)), 4)),
                notice,
                "{}",
                tag.name
            );
            let name = notice.strip_prefix("Picked up ").unwrap();
            let name = ["a ", "an ", "the "]
                .into_iter()
                .find_map(|article| name.strip_prefix(article))
                .unwrap_or(name);
            if !is_weapon {
                let subject = subjects
                    .chunks_exact(8)
                    .find(|subject| word(subject, 0) == id.0)
                    .expect("the power-up subject follows its rebuilt datum");
                assert_eq!(row_text(half(subject, 4)), name);
                checked.1 += 1;
                continue;
            }
            checked.0 += 1;
            let wphi = TagRef::read(src, 0x480).unwrap().id;
            let has_sprite = !wphi.is_none()
                && half(h1.tag_data(h1.tag(wphi).unwrap()).unwrap(), 0x13c) != u16::MAX;
            for field in [0xcc, 0xd0, 0xd4, 0xd8, 0xdc, 0xec] {
                let route = route(word(&built, field));
                let template = row(half(route, 4));
                let (action, button) = match field {
                    0xd0 | 0xd8 => (b"swap_weapon".as_slice(), 18),
                    0xec => (b"remind_to_switch_weapons".as_slice(), 24),
                    _ => (b"pickup".as_slice(), 18),
                };
                assert_eq!(&template[..action.len()], action);
                assert_eq!(template[action.len()], 0);
                let first = usize::from(half(template, 0x22));
                let count = usize::from(template[0x24]);
                let glyphs: Vec<_> = parts[first * 2..(first + count) * 2]
                    .chunks_exact(2)
                    .filter(|part| part[0] == 1)
                    .map(|part| part[1])
                    .collect();
                assert_eq!(glyphs, [button, 32]);
                let arg = row_text(half(route, 6));
                if has_sprite {
                    let (label, icon) = arg.rsplit_once(' ').unwrap();
                    assert_eq!(label, name);
                    let glyph = u32::from(icon.chars().next().unwrap());
                    let index = glyph.checked_sub(0xe500).unwrap() as usize;
                    let sprite = &sprites[index * 0x18..][..0x18];
                    assert_eq!(
                        word(sprite, 0x10),
                        2 * u32::from(half(sprite, 4)) * u32::from(half(sprite, 6))
                    );
                    assert!(
                        block(&mut out, sprite, 0x10, 1)
                            .iter()
                            .any(|&byte| byte != 0)
                    );
                } else {
                    assert_eq!(arg, name);
                }
            }
        }
        assert_eq!(checked.1, 3, "{sandbox:?}");
        if sandbox == Sandbox::Halo1 {
            assert_eq!(checked.0, 8, "converted Blood Gulch weapons");
        } else {
            assert_eq!(checked.0, 0);
            assert!(sprites.is_empty());
            let rifle = out
                .read_tag_bytes(TagQuery::NameInGroup(
                    r"objects\weapons\rifle\battle_rifle\battle_rifle",
                    FourCC(*b"weap"),
                ))
                .unwrap();
            let hudg = out
                .read_tag_bytes(TagQuery::NameInGroup(r"ui\hud\default", FourCC(*b"hudg")))
                .unwrap();
            let unic = Unic::read(&mut out, TagId(word(&hudg, 0x3fc))).unwrap();
            let locales = Localization::read(&mut out).unwrap();
            let prompt = locales
                .text_in(&unic, StringId(word(&rifle, 0xcc)), Language::English)
                .unwrap()
                .unwrap();
            assert!(
                prompt.contains('\u{e448}') && prompt.contains('\u{e112}'),
                "{prompt:?}"
            );
        }
    }
}
