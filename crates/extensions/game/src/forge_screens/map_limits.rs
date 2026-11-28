//! The read-only Map limits page, fed by the same readers as `forge.capacity`.

use guest_layout::GuestMemory;
use forge::BlockKind;

use super::sheet::{Act, Kind, Sheet, SheetRow};

const BLOCKS: [(BlockKind, &str); 6] = [
    (BlockKind::Scenery, "Scenery"),
    (BlockKind::Crates, "Crates"),
    (BlockKind::Machines, "Machines"),
    (BlockKind::NetgameEquipment, "Item spawners"),
    (BlockKind::StartingLocations, "Starting points"),
    (BlockKind::NetgameFlags, "Goals"),
];

type Count = (u32, Option<u32>);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    objects: Option<Count>,
    blocks: [Option<Count>; BLOCKS.len()],
}

impl Usage {
    pub(crate) fn read(memory: &dyn GuestMemory, scenario: u32) -> Self {
        Self {
            objects: agent::forge::object_pool(memory)
                .ok()
                .map(|pool| (pool.used, Some(pool.limit))),
            blocks: BLOCKS.map(|(block, _)| {
                agent::forge::block_usage(memory, scenario, block.name()).ok()
            }),
        }
    }

    pub(super) fn sample() -> Self {
        Self {
            objects: Some((12, Some(24))),
            blocks: [
                Some((4, Some(16))),
                Some((0, Some(8))),
                Some((3, Some(3))),
                Some((2, Some(10))),
                Some((6, None)),
                Some((2, None)),
            ],
        }
    }
}

pub(super) fn sheet(usage: &Usage) -> Sheet {
    let rows = std::iter::once(("Objects", usage.objects))
        .chain(
            BLOCKS
                .into_iter()
                .zip(usage.blocks)
                .map(|((_, label), count)| (label, count)),
        )
        .map(|(label, count)| {
            let row = SheetRow::new(label, Act::Info);
            match count {
                Some((used, Some(limit))) => row.value(&format!("{used} / {limit}")),
                Some((used, None)) => row.value(&used.to_string()),
                None => row.unavailable(),
            }
        })
        .collect();
    Sheet {
        kind: Kind::MapLimits,
        title: "MAP LIMITS".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge_screens::{Facts, prepare, publish, screen};
    use crate::ui_kit::spec::Describe;
    use guest_layout::datum::layout::{ACTIVE_COUNT, MAXIMUM_COUNT};
    use guest_layout::testing::FakeGuest;
    use guest_layout::global;
    use map::scenario_layout;

    #[test]
    fn map_limits_read_each_live_count_and_preserve_unknown_limits() {
        let mut memory = FakeGuest::new();
        let (scenario, pool) = (0x1000, 0x3000);
        let crates = scenario_layout::block(BlockKind::Crates);
        let scenery = scenario_layout::block(BlockKind::Scenery);
        let starts = scenario_layout::block(BlockKind::StartingLocations);
        let spawners = scenario_layout::block(BlockKind::NetgameEquipment);
        memory
            .put_u32(global::OBJECTS.base, pool)
            .put_u32(pool + ACTIVE_COUNT, 7)
            .put_u32(pool + MAXIMUM_COUNT, 19)
            // The block readers need the extent, so no row payload is mapped.
            .put_u32(scenario + crates.offset, 3)
            .put_u32(scenario + crates.offset + 4, 0x5000)
            .put_u32(scenario + starts.offset, 5)
            .put_u32(scenario + starts.offset + 4, 0x6000)
            // A nonempty block without a pointer is unreadable.
            .put_u32(scenario + spawners.offset, 1);

        let page = sheet(&Usage::read(&memory, scenario));
        let want = [
            ("Objects", "7 / 19".into()),
            ("Scenery", format!("0 / {}", scenery.max_count.unwrap())),
            ("Crates", format!("3 / {}", crates.max_count.unwrap())),
            (
                "Machines",
                format!(
                    "0 / {}",
                    scenario_layout::block(BlockKind::Machines).max_count.unwrap()
                ),
            ),
            ("Item spawners", "Unavailable".into()),
            ("Starting points", "5".into()),
            ("Goals", "0".into()),
        ];
        assert_eq!(page.rows.len(), want.len());
        for (row, (label, value)) in page.rows.iter().zip(want) {
            assert_eq!(row.label, label);
            assert_eq!(row.value.as_deref(), Some(value.as_str()));
            assert_eq!(row.act, Act::Info);
        }

        memory.put_u32(pool + ACTIVE_COUNT, 9);
        publish(Facts {
            active: true,
            map_limits: Usage::read(&memory, scenario),
            ..Facts::default()
        });
        prepare(Kind::MapLimits).unwrap();
        let reopened = screen(Kind::MapLimits);
        assert_eq!(reopened.sheet().rows[0].value.as_deref(), Some("9 / 19"));
        assert_eq!(reopened.spec().content.unavailable_values, ["row.4.value"]);
    }
}
