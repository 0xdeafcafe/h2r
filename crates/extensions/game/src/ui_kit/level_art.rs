//! Map pictures referenced by the current main menu's UI level lists.
//! Both Theater and its disc preview use the same scenario lookup.

use std::collections::BTreeMap;

use guest_layout::memory::GuestMemory;
use tags::ui_levels::{self, CampaignLevel, MpLevel};

pub(crate) fn stem(path: &str) -> String {
    let file = ui_levels::stem(path.trim())
        .unwrap_or("")
        .to_ascii_lowercase();
    file.strip_suffix(".map").unwrap_or(&file).to_owned()
}

pub(crate) fn pictures(
    mp: Vec<MpLevel>,
    campaign: Vec<CampaignLevel>,
    unknown_map: Option<u32>,
) -> BTreeMap<String, u32> {
    let mut pictures = BTreeMap::new();
    for (path, datum) in mp
        .into_iter()
        .map(|level| (level.path, level.picture))
        .chain(
            campaign
                .into_iter()
                .map(|level| (level.path, level.picture)),
        )
    {
        if let Some(datum) = datum.filter(|d| Some(*d) != unknown_map) {
            let stem = stem(&path);
            if !stem.is_empty() {
                pictures.entry(stem).or_insert(datum);
            }
        }
    }
    pictures
}

pub(crate) fn loaded(m: &dyn GuestMemory, unknown_map: u32) -> BTreeMap<String, u32> {
    let mut matg = [0; ui_levels::MATG_LEVELS + 8];
    let globals = m.u32(crate::mod_slots::GLOBALS_TAG).unwrap_or(0);
    if globals == 0 || m.read(globals, &mut matg).is_err() {
        return BTreeMap::new();
    }
    let mut fetch = |address, size| {
        let mut bytes = vec![0; size as usize];
        m.read(address, &mut bytes).ok()?;
        Some(bytes)
    };
    pictures(
        ui_levels::mp_levels(&matg, &mut fetch).unwrap_or_default(),
        ui_levels::campaign_levels(&matg, &mut fetch).unwrap_or_default(),
        Some(unknown_map),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_slots_supply_their_own_art_and_skip_the_lobby_placeholder() {
        use guest_layout::testing::FakeGuest;

        let mut g = FakeGuest::new();
        let (matg, ui, slots) = (0x1000, 0x2000, 0x3000);
        g.map(crate::mod_slots::GLOBALS_TAG, 4)
            .put_u32(crate::mod_slots::GLOBALS_TAG, matg)
            .map(matg, (ui_levels::MATG_LEVELS + 8) as u32)
            .put_u32(matg + ui_levels::MATG_LEVELS as u32, 1)
            .put_u32(matg + ui_levels::MATG_LEVELS as u32 + 4, ui)
            .map(ui, 0x18)
            .put_u32(ui + ui_levels::LEVEL_DATA_MP as u32, 3)
            .put_u32(ui + ui_levels::LEVEL_DATA_MP as u32 + 4, slots)
            .map(slots, 3 * ui_levels::MP_LEVEL_SIZE as u32);
        for (i, path, datum) in [
            (0, "Maps/MOCK-ARENA.map", 100),
            (1, "maps\\no-art", 200),
            (2, "maps\\no-datum", u32::MAX),
        ] {
            let slot = slots + i * ui_levels::MP_LEVEL_SIZE as u32;
            g.put_u32(slot, i)
                .put_u32(slot + ui_levels::MP_PICTURE as u32, datum);
            g.write(slot + ui_levels::MP_PATH as u32, path.as_bytes())
                .unwrap();
        }
        assert_eq!(
            loaded(&g, 200),
            BTreeMap::from([("mock-arena".into(), 100)])
        );

        assert_eq!(
            pictures(
                vec![],
                vec![CampaignLevel {
                    campaign: 9,
                    map_id: 12,
                    name: "Test".into(),
                    description: String::new(),
                    path: "scenarios\\solo\\Mission".into(),
                    picture: Some(300),
                }],
                None
            ),
            BTreeMap::from([("mission".into(), 300)])
        );
    }
}
